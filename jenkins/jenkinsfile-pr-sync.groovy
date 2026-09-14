pipeline {
    agent { label 'ubuntu' }

    options {
        skipDefaultCheckout()
    }

    triggers {
        cron('H H/8 * * *')
    }

    environment {
        GITEA_SERVER    = 'http://git.kvaser.se'
        GITEA_OWNER     = 'kvaser'
        GITEA_REPO      = 'canking-ext-xcp-ccp'
        GITHUB_OWNER    = 'Kvaser'
        GITHUB_REPO     = 'canking-ext-xcp-ccp'
        BASE_BRANCH     = 'develop'
    }

    stages {
        stage('Checkout') {
            steps {
                checkout scm
            }
        }

        stage('Fetch PR refs from GitHub') {
            steps {
                withCredentials([string(credentialsId: 'gh-api-token', variable: 'GH_API_TOKEN')]) {
                    sh '''
                        auth=$(printf "x-access-token:%s" "$GH_API_TOKEN" | base64 -w0)
                        git -c http.extraheader="Authorization: Basic ${auth}" \
                          fetch "https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}.git" \
                          "+refs/pull/*/head:refs/remotes/github/pr/*" --prune
                    '''
                }
            }
        }

        stage('Sync PRs into Gitea') {
            steps {
                sshagent(credentials: ['c5f5ee40-6015-4aa2-9016-09836c7b378c']) {
                    withCredentials([
                        usernamePassword(credentialsId: 'b041a79d-d7ae-4d9a-b199-e317ac8b228d', usernameVariable: 'GITEA_USER', passwordVariable: 'GITEA_TOKEN'),
                        string(credentialsId: 'gh-api-token', variable: 'GITHUB_TOKEN')
                    ]) {
                        sh '''
                            export GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no"
                            python3 jenkins/scripts/sync_github_prs.py
                        '''
                    }
                }
            }
        }
    }
}
